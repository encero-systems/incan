//! What a generic element read and a trait method's `mut` parameter need from the generated Rust: an index read or a
//! slice of type-parameter elements states the `Clone` capability its copy needs, on a function, a trait slot and its
//! implementations (#1756), and a `mut` aggregate parameter keeps one Rust shape across the trait slot, every
//! implementation and the recoverable wrapper (#1773).

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

/// Return the bounds of type parameter `param` on method `method` of trait `trait_name` and of every implementation
/// of that trait's method, slot first.
fn trait_method_param_bounds(
    ir: &IrProgram,
    trait_name: &str,
    method: &str,
    param: &str,
) -> Result<Vec<Vec<String>>, String> {
    let bounds_of = |function: &IrFunction| -> Result<Vec<String>, String> {
        function
            .type_params
            .iter()
            .find(|type_param| type_param.name == param)
            .map(|type_param| type_param.bounds.iter().map(|bound| bound.trait_path.clone()).collect())
            .ok_or_else(|| format!("`{}` has no type parameter `{param}`", function.name))
    };
    let mut found = Vec::new();
    for decl in &ir.declarations {
        match &decl.kind {
            IrDeclKind::Trait(trait_decl) if trait_decl.name == trait_name => {
                for function in trait_decl.methods.iter().filter(|function| function.name == method) {
                    found.insert(0, bounds_of(function)?);
                }
            }
            IrDeclKind::Impl(impl_block) if impl_block.trait_name.as_deref() == Some(trait_name) => {
                for function in impl_block.methods.iter().filter(|function| function.name == method) {
                    found.push(bounds_of(function)?);
                }
            }
            _ => {}
        }
    }
    Ok(found)
}

/// #1756: a list slice copies elements like an index read, so a generic function slicing a `list[K]` states `Clone`;
/// a trait method whose default reads or slices a `list[K]` (or reads a `dict[str, K]` value) states it on the trait
/// slot, adopted or not, as well as on each expanded implementation, and a slot filled by an adopter's own
/// element-reading body admits that body's `Clone`.
#[test]
fn trait_method_and_slice_element_copies_state_clone_on_every_signature_issue1756() -> Result<(), String> {
    let ir = lower_with_inferred_bounds(
        r#"
def rest[K](items: list[K]) -> list[K]:
    return items[1:]

def counts(items: list[int]) -> list[int]:
    return items[1:]

trait Picker:
    def pick[K](self, items: list[K]) -> K:
        return items[0]

    def tail[K](self, items: list[K]) -> list[K]:
        return items[1:]

    def chosen[K](self, items: list[K]) -> K

model Chooser with Picker:
    id: int

    def chosen[K](self, items: list[K]) -> K:
        return items[0]

trait Unadopted:
    def first_of[K](self, table: dict[str, K], key: str) -> K:
        return table[key]
"#,
    )?;
    assert_eq!(
        trait_method_param_bounds(&ir, "Unadopted", "first_of", "K")?,
        [vec![tb::CLONE.to_string()]],
        "a default's element read states `Clone` on the slot itself, for adopters in other modules"
    );
    assert!(type_param_has_bound(function(&ir, "rest")?, "K", tb::CLONE)?);
    assert!(function(&ir, "counts")?.type_params.is_empty());
    for method in ["pick", "tail", "chosen"] {
        let signatures = trait_method_param_bounds(&ir, "Picker", method, "K")?;
        assert_eq!(
            signatures.len(),
            2,
            "`{method}` has a slot and one implementation: {signatures:?}"
        );
        assert!(
            signatures
                .iter()
                .all(|bounds| bounds.iter().filter(|bound| *bound == tb::CLONE).count() == 1),
            "the slot and the implementation of `{method}` both state `Clone` once: {signatures:?}"
        );
    }
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

/// #1773: a scalar `mut` parameter the body changes is a mutable local of that body in a function, an inherent method and
/// an expanded trait default, while the trait slot and the wrapper keep taking the value.
#[test]
fn scalar_mut_parameter_is_a_mutable_local_in_every_body_issue1773() -> Result<(), Box<dyn std::error::Error>> {
    let rust = compact_rust(
        r#"
def bumped(mut n: int) -> int:
    n += 1
    n = n * 2
    return n

class Counter:
    step: int

    def advanced(self, mut n: int) -> int:
        n += self.step
        return n

trait Stepper:
    def stepped(self, mut n: int) -> int:
        n += 1
        return n

model Walker with Stepper:
    id: int

def main() -> None:
    println(bumped(1))
    println(Counter(step=2).advanced(1))
    println(Walker(id=1).stepped(1))
"#,
    )?;
    assert!(
        rust.contains("mutn:i64,)->i64{n=n+1;n=n*2;returnn;}"),
        "the function's `mut n` is a mutable local: {rust}"
    );
    assert!(
        rust.contains("&self,mutn:i64,)->i64{n=n+self.step;returnn;}"),
        "the method's `mut n` is a mutable local: {rust}"
    );
    assert!(
        rust.contains("fnstepped(&self,n:i64)->i64;")
            && rust.contains("fnstepped(&self,mutn:i64)->i64{n=n+1;returnn;}"),
        "the trait slot takes the value and the expanded default binds it mutably: {rust}"
    );
    Ok(())
}

/// #1773: a call through a generic bound passes a caller-visible `mut` argument the way the trait slot takes it,
/// an immutable binding for a parameter the callee never changes is passed as a copy, and a `mut` parameter of an
/// alias of `int` is the callee's own copy, passed by value.
#[test]
fn mut_arguments_follow_the_checker_facts_issue1773() -> Result<(), Box<dyn std::error::Error>> {
    let rust = compact_rust(
        r#"
type Count = int

trait Grower:
    def grow(self, mut items: list[int]) -> int:
        items.append(1)
        return len(items)

model Plant with Grower:
    id: int

def run[T with Grower](g: T, mut items: list[int]) -> int:
    return g.grow(items)

def total(mut items: list[int]) -> int:
    return len(items)

def bump(mut n: Count) -> Count:
    n += 1
    return n

def main() -> None:
    mut items: list[int] = []
    println(run(Plant(id=1), items))
    fixed: list[int] = [1, 2]
    println(total(fixed))
    c: Count = 1
    println(bump(c))
"#,
    )?;
    assert!(
        rust.contains(".grow(items)") && !rust.contains("items.clone())"),
        "the generic call hands its `mut` parameter on without a copy: {rust}"
    );
    assert!(
        rust.contains("(&mutfixed.clone())"),
        "an immutable binding for an unchanged `mut` parameter is passed as a copy: {rust}"
    );
    assert!(
        rust.contains("mutn:Count") && rust.contains("(c))") && !rust.contains("&mutc"),
        "a `mut` parameter of an `int` alias is the callee's own copy: {rust}"
    );
    Ok(())
}

/// #1773: a call through a callable known only by its type, `(mut Counter) -> int`, passes the caller's value for the
/// marked parameter, whether the callable is a local bound to a function, a callable-typed parameter or an annotated
/// local, so the callee's change reaches the caller.
#[test]
fn marked_arguments_through_callable_types_pass_the_callers_value_issue1773() -> Result<(), Box<dyn std::error::Error>>
{
    let rust = compact_rust(
        r#"
class Counter:
    pub value: int

def grow(mut counter: Counter) -> int:
    counter.value += 1
    return counter.value

def apply(step: (mut Counter) -> int, mut counter: Counter) -> int:
    return step(counter)

def main() -> None:
    mut counter = Counter(value=1)
    g = grow
    println(g(counter))
    println(apply(grow, counter))
    handler: (mut Counter) -> int = grow
    println(handler(counter))
"#,
    )?;
    assert!(
        rust.contains("step:fn(&mutCounter)->i64") && rust.contains("returnstep(counter);"),
        "a callable-typed parameter takes and passes the caller's value: {rust}"
    );
    assert!(
        rust.contains("g(&mutcounter)") && rust.contains("handler(&mutcounter)"),
        "a call through a local of callable type passes the caller's value: {rust}"
    );
    Ok(())
}
